//! A minimal scriptable HTTP/1.1 server for exercising the internal engine.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One received request.
#[derive(Debug, Clone)]
pub struct Request {
    /// Request target: a path, or an absolute URL when sent to a proxy.
    pub target: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The requested byte range `(start, end)`.
    pub fn range(&self) -> Option<(u64, Option<u64>)> {
        let spec = self.header("range")?.strip_prefix("bytes=")?;
        let (start, end) = spec.split_once('-')?;
        Some((start.parse().ok()?, end.parse().ok()))
    }

    pub fn path(&self) -> &str {
        match self.target.find("://") {
            Some(scheme_end) => {
                let rest = &self.target[scheme_end + 3..];
                rest.find('/').map_or("/", |slash| &rest[slash..])
            }
            None => &self.target,
        }
    }
}

/// How the body length is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// `Content-Length` of the body.
    Length,
    /// No length: the body ends when the connection closes.
    CloseDelimited,
}

/// A scripted response.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub framing: Framing,
    /// Body bytes per write.
    pub write_size: usize,
    /// Pause before each write.
    pub write_delay: Duration,
    /// Close the connection after this many body bytes.
    pub abort_after: Option<usize>,
    /// Stop sending for the duration after this many body bytes.
    pub stall_after: Option<(usize, Duration)>,
    /// Close the connection without answering.
    pub hang_up: bool,
}

impl Reply {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            framing: Framing::Length,
            write_size: 16 * 1024,
            write_delay: Duration::ZERO,
            abort_after: None,
            stall_after: None,
            hang_up: false,
        }
    }

    /// Closes the connection after reading the request, before any response byte.
    pub fn hang_up() -> Self {
        Self {
            hang_up: true,
            ..Self::status(200)
        }
    }

    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// How the file route serves its file.
#[derive(Debug, Clone)]
pub struct FileOptions {
    /// Answer range requests with `206 Partial Content`.
    pub ranges: bool,
    pub framing: Framing,
    pub write_size: usize,
    pub write_delay: Duration,
}

impl Default for FileOptions {
    fn default() -> Self {
        Self {
            ranges: true,
            framing: Framing::Length,
            write_size: 16 * 1024,
            write_delay: Duration::ZERO,
        }
    }
}

/// The reply serving `data` for `request` according to `options`.
pub fn file_reply(data: &[u8], request: &Request, options: &FileOptions) -> Reply {
    let size = u64::try_from(data.len()).expect("size fits");
    let mut reply = Reply::status(200);
    reply.framing = options.framing;
    reply.write_size = options.write_size;
    reply.write_delay = options.write_delay;
    if let Some((start, end)) = request.range().filter(|_| options.ranges) {
        let end = end.unwrap_or(size - 1).min(size - 1);
        if start > end {
            return Reply::status(416).header("Content-Range", &format!("bytes */{size}"));
        }
        let index = |offset: u64| usize::try_from(offset).expect("offset fits");
        reply.status = 206;
        reply.body = data[index(start)..=index(end)].to_vec();
        reply = reply
            .header("Accept-Ranges", "bytes")
            .header("Content-Range", &format!("bytes {start}-{end}/{size}"));
    } else {
        reply.body = data.to_vec();
        if options.ranges {
            reply = reply.header("Accept-Ranges", "bytes");
        }
    }
    reply
}

type Handler = dyn Fn(&Request) -> Reply + Send + Sync;

/// A running test server.
#[derive(Debug)]
pub struct TestServer {
    pub addr: SocketAddr,
    stats: Arc<Stats>,
}

#[derive(Debug, Default)]
struct Stats {
    requests: Mutex<Vec<Request>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl TestServer {
    /// Starts a server answering every request with `handler`.
    pub async fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let stats = Arc::new(Stats::default());
        let handler: Arc<Handler> = Arc::new(handler);
        let server_stats = Arc::clone(&stats);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = Arc::clone(&handler);
                let stats = Arc::clone(&server_stats);
                tokio::spawn(async move {
                    serve(stream, &*handler, &stats).await;
                });
            }
        });
        Self { addr, stats }
    }

    /// Starts a server serving `data` on every path.
    pub async fn file(data: Arc<Vec<u8>>, options: FileOptions) -> Self {
        Self::start(move |request| file_reply(&data, request, &options)).await
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.stats.requests.lock().expect("requests").clone()
    }

    /// Requests other than the `bytes=0-0` probe.
    pub fn transfer_requests(&self) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|request| request.range() != Some((0, Some(0))))
            .collect()
    }

    /// Highest number of transfer requests (not probes) served at the same time.
    pub fn max_concurrent(&self) -> usize {
        self.stats.max_active.load(Ordering::SeqCst)
    }
}

async fn serve(stream: TcpStream, handler: &Handler, stats: &Stats) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let request = Request { target, headers };
    stats
        .requests
        .lock()
        .expect("requests")
        .push(request.clone());
    // Probes are short-lived and overlap the first transfers; only transfers are counted.
    let mut slot = ActiveSlot::take(stats, request.range() != Some((0, Some(0))));
    let reply = handler(&request);
    let mut stream = reader.into_inner();
    let _ = write_reply(&mut stream, &reply, &mut slot).await;
    drop(slot);
    let _ = stream.shutdown().await;
}

/// A transfer counted as active until it releases the slot or is dropped.
struct ActiveSlot<'a>(Option<&'a AtomicUsize>);

impl<'a> ActiveSlot<'a> {
    fn take(stats: &'a Stats, counted: bool) -> Self {
        if !counted {
            return Self(None);
        }
        let active = stats.active.fetch_add(1, Ordering::SeqCst) + 1;
        stats.max_active.fetch_max(active, Ordering::SeqCst);
        Self(Some(&stats.active))
    }

    fn release(&mut self) {
        if let Some(active) = self.0.take() {
            active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for ActiveSlot<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// Writes `reply`. `slot` is released just before the last body bytes go out: once the client
/// has them it may open its next connection before this task runs again.
async fn write_reply(
    stream: &mut TcpStream,
    reply: &Reply,
    slot: &mut ActiveSlot<'_>,
) -> std::io::Result<()> {
    if reply.hang_up {
        return Ok(());
    }
    let mut head = format!(
        "HTTP/1.1 {} Scripted\r\nConnection: close\r\n",
        reply.status
    );
    for (name, value) in &reply.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if reply.framing == Framing::Length {
        let _ = write!(head, "Content-Length: {}\r\n", reply.body.len());
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    let mut sent = 0;
    let mut stalled = false;
    while sent < reply.body.len() {
        if let Some(limit) = reply.abort_after
            && sent >= limit
        {
            return Ok(());
        }
        if let Some((after, duration)) = reply.stall_after
            && sent >= after
            && !stalled
        {
            stalled = true;
            tokio::time::sleep(duration).await;
        }
        if !reply.write_delay.is_zero() {
            tokio::time::sleep(reply.write_delay).await;
        }
        let mut end = (sent + reply.write_size).min(reply.body.len());
        if let Some(limit) = reply.abort_after {
            end = end.min(limit.max(sent + 1));
        }
        if let Some((after, _)) = reply.stall_after
            && !stalled
            && after > sent
        {
            end = end.min(after);
        }
        if end == reply.body.len() {
            slot.release();
        }
        stream.write_all(&reply.body[sent..end]).await?;
        stream.flush().await?;
        sent = end;
    }
    Ok(())
}

/// `len` bytes of deterministic, non-repeating-looking content.
pub fn test_data(len: usize) -> Arc<Vec<u8>> {
    Arc::new(
        (0..len)
            .map(|index| (index.wrapping_mul(31) ^ (index >> 8)).to_le_bytes()[0])
            .collect(),
    )
}
